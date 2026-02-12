// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::backend::TreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPathBuf;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::command_error::internal_error_with_message;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::ui::Ui;

/// Run a Jujutsu command in a Git submodule
///
/// The first invocation initializes a colocated Jujutsu repository inside the
/// selected Git submodule. Files are snapshotted into that nested repository,
/// so they can be inspected and committed independently of the superproject.
/// The superproject records the nested working-copy commit as its gitlink while
/// it contains changes, making the submodule path an ordinary outer change.
///
/// When run from inside a submodule, the path is inferred:
///
/// ```shell
/// jj sub status
/// jj sub commit -m 'Describe the nested change'
/// ```
///
/// From the superproject root, select a submodule with `-S`:
///
/// ```shell
/// jj sub -S path/to/submodule status
/// ```
///
/// If a nested repository gets into an unusable state, `jj sub --reset`
/// restores its gitlink from the superproject working-copy commit's parents.
/// `jj sub --reset-all` does the same recursively for every submodule. Nested
/// Jujutsu metadata is preserved below the superproject's `.jj` directory so
/// it can be recovered later.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub(crate) struct SubArgs {
    /// Submodule repository to operate on
    #[arg(short = 'S', long = "submodule", value_hint = clap::ValueHint::DirPath)]
    submodule_repository: Option<PathBuf>,

    /// Reset the selected submodule, preserving nested Jujutsu metadata
    #[arg(long, conflicts_with_all = ["reset_all", "command"])]
    reset: bool,

    /// Reset all submodules, preserving nested Jujutsu metadata as backups
    #[arg(
        long,
        conflicts_with_all = ["submodule_repository", "reset", "command"]
    )]
    reset_all: bool,

    /// Jujutsu command and arguments to run (defaults to `status`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<OsString>,
}

#[instrument(skip_all)]
pub(crate) async fn cmd_sub(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &SubArgs,
) -> Result<(), CommandError> {
    // Don't snapshot first. If the nested Git HEAD was moved directly, this
    // command must initialize the nested repo before the superproject records
    // that HEAD as a gitlink update.
    let workspace_command = command.workspace_helper_no_snapshot(ui).await?;
    if !workspace_command.working_copy_shared_with_git() {
        return Err(user_error(
            "Git submodules are supported only in colocated Git workspaces.",
        ));
    }

    if args.reset_all {
        let workspace_root = workspace_command.workspace_root();
        if workspace_root.join(".git").is_file()
            && let Some(superproject_root) = find_superproject_root(workspace_root)
        {
            return dispatch_reset_to_superproject(&superproject_root, None);
        }
        return reset_submodules(ui, command, &workspace_command, None).await;
    }

    if args.reset {
        let workspace_root = workspace_command.workspace_root();
        if workspace_root.join(".git").is_file()
            && let Some(superproject_root) = find_superproject_root(workspace_root)
            && (args.submodule_repository.is_none()
                || args
                    .submodule_repository
                    .as_ref()
                    .is_some_and(|repository| {
                        requested_repository_matches(
                            command,
                            repository,
                            workspace_command.workspace_root(),
                        )
                    }))
        {
            let relative_path = workspace_root
                .strip_prefix(&superproject_root)
                .expect("superproject is an ancestor");
            return dispatch_reset_to_superproject(&superproject_root, Some(relative_path));
        }
        let selected = select_reset_submodule(command, &workspace_command, args).await?;
        return reset_submodules(ui, command, &workspace_command, Some(selected)).await;
    }

    let (submodule_path, submodule_root, superproject_root) =
        select_submodule(command, &workspace_command, args).await?;
    let jj_executable = std::env::current_exe()
        .map_err(|err| internal_error_with_message("Could not find the jj executable", err))?;

    if !submodule_root.join(".jj").exists() {
        let status = std::process::Command::new(&jj_executable)
            .current_dir(&submodule_root)
            .args(["git", "init", "--colocate", "."])
            .status()
            .map_err(|err| {
                user_error_with_message(
                    format!(
                        "Failed to initialize Jujutsu in submodule {}",
                        submodule_path.as_internal_file_string()
                    ),
                    err,
                )
            })?;
        if !status.success() {
            return Err(user_error(format!(
                "Could not initialize Jujutsu in submodule {}.",
                submodule_path.as_internal_file_string()
            )));
        }
    }

    let nested_args = if args.command.is_empty() {
        vec![OsString::from("status")]
    } else {
        args.command.clone()
    };
    let status = std::process::Command::new(&jj_executable)
        .current_dir(&submodule_root)
        .args(nested_args)
        .status()
        .map_err(|err| {
            user_error_with_message(
                format!(
                    "Failed to run Jujutsu in submodule {}",
                    submodule_path.as_internal_file_string()
                ),
                err,
            )
        })?;
    if !status.success() {
        return Err(user_error(format!(
            "Jujutsu command in submodule {} exited with {status}.",
            submodule_path.as_internal_file_string()
        )));
    }

    if superproject_root == workspace_command.workspace_root() {
        // Snapshotting now records the nested working-copy commit as the
        // superproject gitlink while it contains changes.
        command.workspace_helper(ui).await?;
    } else if let Some(superproject_root) = superproject_root.to_str() {
        // If this command was started after the nested .jj repository already
        // existed, the current CommandHelper belongs to that nested repo. Ask
        // the outer workspace to snapshot the resulting gitlink too.
        let status = std::process::Command::new(&jj_executable)
            .args([
                "--quiet",
                "--repository",
                superproject_root,
                "util",
                "snapshot",
            ])
            .status()
            .map_err(|err| user_error_with_message("Failed to snapshot the superproject", err))?;
        if !status.success() {
            return Err(user_error(format!(
                "Could not snapshot superproject {superproject_root}."
            )));
        }
    }

    Ok(())
}

fn dispatch_reset_to_superproject(
    superproject_root: &Path,
    submodule: Option<&Path>,
) -> Result<(), CommandError> {
    let jj_executable = std::env::current_exe()
        .map_err(|err| internal_error_with_message("Could not find the jj executable", err))?;
    let mut child = std::process::Command::new(jj_executable);
    child
        .current_dir(superproject_root)
        .args(["--repository"])
        .arg(superproject_root)
        .arg("sub");
    if let Some(submodule) = submodule {
        child.arg("-S").arg(submodule).arg("--reset");
    } else {
        child.arg("--reset-all");
    }
    let status = child.status().map_err(|err| {
        user_error_with_message(
            format!(
                "Failed to reset submodules in superproject {}",
                superproject_root.display()
            ),
            err,
        )
    })?;
    if !status.success() {
        return Err(user_error(format!(
            "Could not reset submodules in superproject {}.",
            superproject_root.display()
        )));
    }
    Ok(())
}

async fn select_reset_submodule(
    command: &CommandHelper,
    workspace_command: &WorkspaceCommandHelper,
    args: &SubArgs,
) -> Result<RepoPathBuf, CommandError> {
    let workspace_root = workspace_command.workspace_root();
    let wc_commit_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This workspace has no working-copy commit."))?;
    let wc_commit = workspace_command.repo().store().get_commit(wc_commit_id)?;
    let parent_tree = wc_commit
        .parent_tree(workspace_command.repo().as_ref())
        .await?;
    let mut submodules = collect_git_submodules(&wc_commit.tree())?
        .into_keys()
        .collect::<BTreeSet<_>>();
    submodules.extend(collect_git_submodules(&parent_tree)?.into_keys());
    let submodules = submodules.into_iter().collect_vec();
    if submodules.is_empty() {
        return Err(user_error(
            "The working-copy commit does not contain any Git submodules.",
        ));
    }

    if let Some(repository) = &args.submodule_repository {
        let requested = if repository.is_absolute() {
            repository.clone()
        } else {
            command.cwd().join(repository)
        };
        return submodules
            .into_iter()
            .find(|path| {
                path.to_fs_path(workspace_root)
                    .is_ok_and(|disk_path| paths_equal(&requested, &disk_path))
            })
            .ok_or_else(|| {
                user_error(format!(
                    "Path '{}' is not a Git submodule in this working-copy commit.",
                    repository.display()
                ))
            });
    }

    let cwd = command.cwd();
    let mut containing = submodules
        .iter()
        .filter(|path| {
            path.to_fs_path(workspace_root)
                .is_ok_and(|disk_path| cwd.starts_with(disk_path))
        })
        .collect_vec();
    containing.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    if let Some(path) = containing.first() {
        return Ok((*path).clone());
    }
    if submodules.len() == 1 {
        return Ok(submodules.into_iter().next().unwrap());
    }

    let available = submodules
        .iter()
        .map(|path| path.as_internal_file_string())
        .join(", ");
    Err(
        user_error("Could not infer which Git submodule to reset.").hinted(format!(
            "Run `jj sub -S <path> --reset`. Available submodules: {available}"
        )),
    )
}

async fn reset_submodules(
    ui: &mut Ui,
    command: &CommandHelper,
    workspace_command: &WorkspaceCommandHelper,
    selected: Option<RepoPathBuf>,
) -> Result<(), CommandError> {
    let workspace_root = workspace_command.workspace_root();
    let wc_commit_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This workspace has no working-copy commit."))?;
    let wc_commit = workspace_command.repo().store().get_commit(wc_commit_id)?;
    let parent_tree = wc_commit
        .parent_tree(workspace_command.repo().as_ref())
        .await?;

    let current_submodules = collect_git_submodules(&wc_commit.tree())?;
    let mut target_submodules = collect_git_submodules(&parent_tree)?;
    let mut paths = current_submodules
        .keys()
        .chain(target_submodules.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    if paths.is_empty() {
        return Err(user_error(
            "The working-copy commit and its parents do not contain any Git submodules.",
        ));
    }
    if let Some(selected) = selected {
        paths.retain(|path| path == &selected);
        target_submodules.retain(|path, _| path == &selected);
    }

    let jj_executable = std::env::current_exe()
        .map_err(|err| internal_error_with_message("Could not find the jj executable", err))?;
    let backup_root = make_submodule_backup_root(workspace_root)?;
    let nested_workspaces = collect_checked_out_submodule_workspaces(workspace_root, paths.iter())?;
    let mut backups = Vec::new();
    for (path, submodule_root) in nested_workspaces {
        let nested_jj = submodule_root.join(".jj");
        if !nested_jj.exists() {
            continue;
        }

        // Give a healthy nested workspace one last chance to snapshot dirty
        // files. Failure is not fatal: preserving its metadata is precisely
        // what makes this useful for broken repositories too.
        let snapshot_status = std::process::Command::new(&jj_executable)
            .current_dir(&submodule_root)
            .args(["--quiet", "util", "snapshot"])
            .status();
        if !matches!(snapshot_status, Ok(status) if status.success()) {
            writeln!(
                ui.warning_default(),
                "Could not snapshot {}; preserving its existing Jujutsu metadata anyway.",
                path.display()
            )?;
        }

        let backup_jj = backup_root.join(&path).join(".jj");
        let backup_parent = backup_jj
            .parent()
            .expect("a nested .jj path always has a parent");
        std::fs::create_dir_all(backup_parent).map_err(|err| {
            user_error_with_message(
                format!(
                    "Could not create backup directory {}",
                    backup_parent.display()
                ),
                err,
            )
        })?;
        if let Err(err) = std::fs::rename(&nested_jj, &backup_jj) {
            for (original, backup) in backups.iter().rev() {
                drop(std::fs::rename(backup, original));
            }
            return Err(user_error_with_message(
                format!(
                    "Could not preserve nested Jujutsu metadata for {}",
                    path.display()
                ),
                err,
            ));
        }
        backups.push((nested_jj, backup_jj));
    }
    if !backups.is_empty() {
        writeln!(
            ui.status(),
            "Preserved nested Jujutsu metadata at {}.",
            backup_root.display()
        )?;
    }

    // Restore only paths which are or were gitlinks. This intentionally keeps
    // unrelated changes in the outer working-copy commit intact.
    let mut restore = std::process::Command::new(&jj_executable);
    restore
        .current_dir(workspace_root)
        .args(["--repository"])
        .arg(workspace_root)
        .args(["restore", "--"]);
    for path in &paths {
        restore.arg(path.as_internal_file_string());
    }
    let status = restore
        .status()
        .map_err(|err| user_error_with_message("Failed to restore superproject gitlinks", err))?;
    if !status.success() {
        return Err(user_error(
            "Could not restore the superproject's Git submodule paths.",
        ));
    }

    force_checkout_submodules(command, workspace_root, &target_submodules)?;

    writeln!(
        ui.status(),
        "Reset {} Git submodule(s) to the superproject working-copy parent.",
        paths.len()
    )?;
    if !backups.is_empty() {
        writeln!(
            ui.hint_default(),
            "Move a saved <submodule>/.jj directory back into its submodule to recover that nested operation log."
        )?;
    }
    Ok(())
}

fn collect_git_submodules(
    tree: &MergedTree,
) -> Result<BTreeMap<RepoPathBuf, CommitId>, CommandError> {
    let mut submodules = BTreeMap::new();
    for (path, value) in tree.entries() {
        let value = value
            .map_err(|err| internal_error_with_message("Failed to inspect Git submodules", err))?;
        if let Some(TreeValue::GitSubmodule(id)) = value.as_normal() {
            submodules.insert(path, id.clone());
        }
    }
    Ok(submodules)
}

fn make_submodule_backup_root(workspace_root: &Path) -> Result<PathBuf, CommandError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| internal_error_with_message("System clock is before the Unix epoch", err))?
        .as_nanos();
    Ok(workspace_root
        .join(".jj")
        .join("submodule-backups")
        .join(format!("{timestamp}-{}", std::process::id())))
}

fn collect_checked_out_submodule_workspaces<'a>(
    workspace_root: &Path,
    paths: impl IntoIterator<Item = &'a RepoPathBuf>,
) -> Result<Vec<(PathBuf, PathBuf)>, CommandError> {
    let mut workspaces = Vec::new();
    let mut visited = BTreeSet::new();
    for path in paths {
        let disk_path = path
            .to_fs_path(workspace_root)
            .map_err(|err| user_error_with_message("Invalid Git submodule path", err))?;
        collect_submodule_descendants(
            workspace_root,
            PathBuf::from(path.as_internal_file_string()),
            disk_path,
            &mut visited,
            &mut workspaces,
        )?;
    }
    // Snapshot and move children before parents. This preserves a descendant's
    // dirty tree before its parent snapshots the descendant's checked-out HEAD.
    workspaces.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    Ok(workspaces)
}

fn collect_submodule_descendants(
    workspace_root: &Path,
    relative_path: PathBuf,
    submodule_root: PathBuf,
    visited: &mut BTreeSet<PathBuf>,
    workspaces: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), CommandError> {
    if !submodule_root.exists() {
        return Ok(());
    }
    let canonical_root = dunce::canonicalize(&submodule_root).map_err(|err| {
        user_error_with_message(
            format!(
                "Could not inspect checked-out submodule {}",
                relative_path.display()
            ),
            err,
        )
    })?;
    if !canonical_root.starts_with(workspace_root) {
        return Err(user_error(format!(
            "Submodule {} resolves outside the superproject working copy.",
            relative_path.display()
        )));
    }
    if !visited.insert(canonical_root.clone()) {
        return Ok(());
    }
    workspaces.push((relative_path.clone(), canonical_root.clone()));

    if !canonical_root.join(".git").exists() {
        return Ok(());
    }
    let repo = gix::open(&canonical_root).map_err(|err| {
        user_error_with_message(
            format!(
                "Could not open checked-out submodule {}",
                relative_path.display()
            ),
            err,
        )
    })?;
    let Some(submodules) = repo.submodules().map_err(|err| {
        user_error_with_message(
            format!(
                "Could not inspect nested submodules below {}",
                relative_path.display()
            ),
            err,
        )
    })?
    else {
        return Ok(());
    };
    for submodule in submodules {
        let child_path = submodule.path().map_err(|err| {
            user_error_with_message(
                format!(
                    "Could not read a nested submodule path below {}",
                    relative_path.display()
                ),
                err,
            )
        })?;
        let child_path = gix::path::from_bstring(child_path);
        let child_relative_path = relative_path.join(&child_path);
        let child_repo_path = RepoPathBuf::from_relative_path(&child_relative_path)
            .map_err(|err| user_error_with_message("Invalid nested Git submodule path", err))?;
        let child_root = child_repo_path
            .to_fs_path(workspace_root)
            .map_err(|err| user_error_with_message("Invalid nested Git submodule path", err))?;
        if child_root.join(".git").exists() || child_root.join(".jj").exists() {
            collect_submodule_descendants(
                workspace_root,
                child_relative_path,
                child_root,
                visited,
                workspaces,
            )?;
        }
    }
    Ok(())
}

fn force_checkout_submodules(
    command: &CommandHelper,
    workspace_root: &Path,
    target_submodules: &BTreeMap<RepoPathBuf, CommitId>,
) -> Result<(), CommandError> {
    if target_submodules.is_empty() {
        return Ok(());
    }
    let git_executable: PathBuf = command.settings().get("git.executable-path")?;
    let mut update = std::process::Command::new(&git_executable);
    update.current_dir(workspace_root).args([
        "submodule",
        "update",
        "--init",
        "--recursive",
        "--force",
        "--",
    ]);
    for path in target_submodules.keys() {
        update.arg(path.as_internal_file_string());
    }
    let output = update.output().map_err(|err| {
        user_error_with_message("Failed to run Git while resetting submodules", err)
    })?;
    if !output.status.success() {
        return Err(command_output_error(
            "Git could not initialize the submodules being reset",
            &output,
        ));
    }

    for (path, id) in target_submodules {
        let submodule_root = path
            .to_fs_path(workspace_root)
            .map_err(|err| user_error_with_message("Invalid Git submodule path", err))?;
        let output = std::process::Command::new(&git_executable)
            .current_dir(&submodule_root)
            .args(["checkout", "--detach", "--force", &id.hex()])
            .output()
            .map_err(|err| {
                user_error_with_message(
                    format!(
                        "Failed to run Git while resetting submodule {}",
                        path.as_internal_file_string()
                    ),
                    err,
                )
            })?;
        if !output.status.success() {
            return Err(command_output_error(
                &format!(
                    "Git could not reset submodule {}",
                    path.as_internal_file_string()
                ),
                &output,
            ));
        }
    }
    Ok(())
}

fn command_output_error(context: &str, output: &std::process::Output) -> CommandError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    user_error(format!("{context}: {}", stderr.trim()))
}

async fn select_submodule(
    command: &CommandHelper,
    workspace_command: &WorkspaceCommandHelper,
    args: &SubArgs,
) -> Result<(RepoPathBuf, PathBuf, PathBuf), CommandError> {
    // A submodule that has already been initialized has its own nearest .jj
    // directory. Keep `jj sub ...` useful there by dispatching to that repo and
    // locating its superproject above it.
    let workspace_root = workspace_command.workspace_root();
    if workspace_root.join(".git").is_file()
        && let Some(superproject_root) = find_superproject_root(workspace_root)
        && (args.submodule_repository.is_none()
            || args
                .submodule_repository
                .as_ref()
                .is_some_and(|repository| {
                    requested_repository_matches(command, repository, workspace_root)
                }))
    {
        return current_submodule_selection(workspace_root, superproject_root);
    }

    let wc_commit_id = workspace_command
        .get_wc_commit_id()
        .ok_or_else(|| user_error("This workspace has no working-copy commit."))?;
    let wc_commit = workspace_command.repo().store().get_commit(wc_commit_id)?;
    let mut submodules = Vec::new();
    for (path, value) in wc_commit.tree().entries() {
        let value = value
            .map_err(|err| internal_error_with_message("Failed to inspect Git submodules", err))?;
        if matches!(value.as_normal(), Some(TreeValue::GitSubmodule(_))) {
            let disk_path = path
                .to_fs_path(workspace_root)
                .map_err(|err| user_error_with_message("Invalid Git submodule path", err))?;
            submodules.push((path, disk_path));
        }
    }
    if submodules.is_empty() {
        return Err(user_error(
            "The working-copy commit does not contain any Git submodules.",
        ));
    }

    let selected = if let Some(repository) = &args.submodule_repository {
        let requested = if repository.is_absolute() {
            repository.clone()
        } else {
            command.cwd().join(repository)
        };
        let mut containing = submodules
            .iter()
            .filter(|(_, disk_path)| path_is_within(&requested, disk_path))
            .collect_vec();
        containing.sort_by_key(|(_, disk_path)| std::cmp::Reverse(disk_path.components().count()));
        if let Some((path, disk_path)) = containing.first() {
            ((*path).clone(), (*disk_path).clone())
        } else {
            return Err(user_error(format!(
                "Path '{}' is not a Git submodule in this working-copy commit.",
                repository.display()
            )));
        }
    } else {
        let cwd = command.cwd();
        let mut containing = submodules
            .iter()
            .filter(|(_, disk_path)| cwd.starts_with(disk_path))
            .collect_vec();
        containing.sort_by_key(|(_, disk_path)| std::cmp::Reverse(disk_path.components().count()));
        if let Some((path, disk_path)) = containing.first() {
            ((*path).clone(), (*disk_path).clone())
        } else if submodules.len() == 1 {
            submodules.pop().unwrap()
        } else {
            let available = submodules
                .iter()
                .map(|(path, _)| path.as_internal_file_string())
                .join(", ");
            return Err(
                user_error("Could not infer which Git submodule to use.").hinted(format!(
                    "Run `jj sub -S <path> ...`. Available submodules: {available}"
                )),
            );
        }
    };

    Ok((selected.0, selected.1, workspace_root.to_owned()))
}

fn current_submodule_selection(
    workspace_root: &Path,
    superproject_root: PathBuf,
) -> Result<(RepoPathBuf, PathBuf, PathBuf), CommandError> {
    let relative_path = workspace_root
        .strip_prefix(&superproject_root)
        .expect("superproject is an ancestor");
    let repo_path = RepoPathBuf::from_relative_path(relative_path)
        .map_err(|err| user_error_with_message("Invalid Git submodule path", err))?;
    Ok((repo_path, workspace_root.to_owned(), superproject_root))
}

fn requested_repository_matches(
    command: &CommandHelper,
    repository: &Path,
    workspace_root: &Path,
) -> bool {
    let requested = if repository.is_absolute() {
        repository.to_owned()
    } else {
        command.cwd().join(repository)
    };
    paths_equal(&requested, workspace_root)
}

fn path_is_within(path: &Path, directory: &Path) -> bool {
    match (dunce::canonicalize(path), dunce::canonicalize(directory)) {
        (Ok(path), Ok(directory)) => path.starts_with(directory),
        _ => path.starts_with(directory),
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    match (dunce::canonicalize(left), dunce::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn find_superproject_root(submodule_root: &Path) -> Option<PathBuf> {
    let git_file = std::fs::read_to_string(submodule_root.join(".git")).ok()?;
    let git_dir = git_file.strip_prefix("gitdir:")?.trim();
    let git_dir = dunce::canonicalize(submodule_root.join(git_dir)).ok()?;
    for ancestor in submodule_root.ancestors().skip(1) {
        if !ancestor.join(".jj").is_dir() || !ancestor.join(".git").is_dir() {
            continue;
        }
        let modules_dir = dunce::canonicalize(ancestor.join(".git/modules")).ok()?;
        if git_dir.starts_with(modules_dir) {
            return Some(ancestor.to_owned());
        }
    }
    None
}
