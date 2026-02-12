// Copyright 2020 The Jujutsu Authors
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
#[cfg(feature = "git")]
use std::collections::BTreeSet;
#[cfg(feature = "git")]
use std::path::Path;
#[cfg(feature = "git")]
use std::path::PathBuf;

use futures::TryStreamExt as _;
use itertools::Itertools as _;
#[cfg(feature = "git")]
use jj_lib::backend::CommitId;
#[cfg(feature = "git")]
use jj_lib::backend::TreeValue;
use jj_lib::commit::Commit;
use jj_lib::copies::CopyRecords;
use jj_lib::matchers::Matcher;
use jj_lib::merge::Diff;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::revset::RevsetExpression;
use jj_lib::revset::RevsetFilterPredicate;
use jj_lib::tree::TreeMergeExt as _;
use jj_lib::working_copy::SnapshotStats;
use jj_lib::working_copy::UntrackedReason;
use tracing::instrument;

use crate::cli_util::CommandHelper;
#[cfg(feature = "git")]
use crate::cli_util::WorkspaceCommandHelper;
use crate::cli_util::print_conflicted_paths;
use crate::cli_util::print_snapshot_stats;
use crate::cli_util::print_unmatched_explicit_paths;
#[cfg(feature = "git")]
use crate::cli_util::short_commit_hash;
use crate::command_error::CommandError;
#[cfg(feature = "git")]
use crate::command_error::internal_error_with_message;
#[cfg(feature = "git")]
use crate::command_error::user_error_with_message;
use crate::diff_util::DiffFormat;
use crate::diff_util::get_copy_records;
#[cfg(feature = "git")]
use crate::formatter::Formatter;
use crate::formatter::FormatterExt as _;
use crate::ui::Ui;

/// Show high-level repo status [default alias: st]
///
/// This includes:
///
/// * The working copy commit and its parents, and a summary of the changes in
///   the working copy (compared to the merged parents)
///
/// * Conflicts in the working copy
///
/// * Git submodules as part of the working-copy summary, including recursive
///   clean, captured nested-workspace, dirty, and mismatched checkout states
///
/// * [Conflicted bookmarks]
///
/// Note: You can use `jj diff --summary -r <rev>` to see the changed files for
/// a specific revision.
///
/// [Conflicted bookmarks]:
///     https://docs.jj-vcs.dev/latest/bookmarks/#conflicts
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct StatusArgs {
    /// Restrict the status display to these paths
    #[arg(value_name = "FILESETS", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,
}

#[instrument(skip_all)]
pub(crate) async fn cmd_status(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &StatusArgs,
) -> Result<(), CommandError> {
    let (workspace_command, snapshot_stats, _) = command.workspace_helper_with_stats(ui).await?;
    print_snapshot_stats(
        ui,
        &snapshot_stats,
        workspace_command.env().path_converter(),
    )?;
    let repo = workspace_command.repo();
    let maybe_wc_commit = workspace_command
        .get_wc_commit_id()
        .map(|id| repo.store().get_commit(id))
        .transpose()?;
    let fileset_expression = workspace_command.parse_file_patterns(ui, &args.paths)?;
    let matcher = fileset_expression.to_matcher();
    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    let formatter = formatter.as_mut();

    if let Some(wc_commit) = &maybe_wc_commit {
        let status = collect_working_copy_status(repo.as_ref(), wc_commit, snapshot_stats).await?;
        print_unmatched_explicit_paths(
            ui,
            &workspace_command,
            &fileset_expression,
            [&status.tree],
        )?;

        #[cfg(feature = "git")]
        let submodule_statuses = if workspace_command.working_copy_shared_with_git() {
            let git_executable: PathBuf = command.settings().get("git.executable-path")?;
            collect_git_submodule_statuses(
                &status.tree,
                &status.parent_tree,
                workspace_command.workspace_root(),
                &git_executable,
            )?
        } else {
            Vec::new()
        };
        #[cfg(feature = "git")]
        let has_submodule_checkout_changes = submodule_statuses
            .iter()
            .any(GitSubmoduleStatus::has_checkout_changes);
        #[cfg(not(feature = "git"))]
        let has_submodule_checkout_changes = false;

        #[cfg(feature = "git")]
        let mut printed_submodule_statuses = false;
        if !status.has_any_tracked_changes()
            && !status.has_any_untracked_paths()
            && !has_submodule_checkout_changes
        {
            writeln!(formatter, "The working copy has no changes.")?;
        } else {
            if status.has_any_tracked_changes() {
                let mut copy_records = CopyRecords::default();
                for parent in &status.parents {
                    let records =
                        get_copy_records(repo.store(), parent.id(), status.commit.id(), &matcher)
                            .await?;
                    copy_records.add_records(records);
                }
                let diff_renderer = workspace_command.diff_renderer(vec![DiffFormat::Summary]);
                let width = ui.term_width();
                let mut diff_output = vec![];
                diff_renderer
                    .show_diff(
                        ui,
                        ui.new_formatter(&mut diff_output).as_mut(),
                        Diff::new(&status.parent_tree, &status.tree),
                        &matcher,
                        &copy_records,
                        width,
                    )
                    .await?;
                if !diff_output.is_empty() {
                    writeln!(formatter, "Working copy changes:")?;
                    formatter.raw()?.write_all(&diff_output)?;
                    #[cfg(feature = "git")]
                    if !submodule_statuses.is_empty() {
                        print_git_submodule_statuses(
                            formatter,
                            &workspace_command,
                            &submodule_statuses,
                            &matcher,
                            true,
                        )?;
                        printed_submodule_statuses = true;
                    }
                }
            }

            let mut matching_untracked_paths = status.untracked_paths_matching(&matcher).peekable();
            if matching_untracked_paths.peek().is_some() {
                writeln!(formatter, "Untracked paths:")?;
                visit_collapsed_untracked_files(
                    matching_untracked_paths,
                    status.tree.clone(),
                    |path, is_dir| {
                        let ui_path = workspace_command.path_converter().format_file_path(path);
                        writeln!(
                            formatter.labeled("diff").labeled("untracked"),
                            "? {ui_path}{}",
                            if is_dir {
                                std::path::MAIN_SEPARATOR_STR
                            } else {
                                ""
                            }
                        )?;
                        Ok(())
                    },
                )
                .await?;
            }
        }

        #[cfg(feature = "git")]
        if !printed_submodule_statuses && !submodule_statuses.is_empty() {
            print_git_submodule_statuses(
                formatter,
                &workspace_command,
                &submodule_statuses,
                &matcher,
                false,
            )?;
        }

        let template = workspace_command.commit_summary_template();
        write!(formatter, "Working copy  (@) : ")?;
        template.format(&status.commit, formatter)?;
        writeln!(formatter)?;
        for parent in &status.parents {
            //                "Working copy  (@) : "
            write!(formatter, "Parent commit (@-): ")?;
            template.format(parent, formatter)?;
            writeln!(formatter)?;
        }

        if status.commit.has_conflict() {
            let conflicts = status.tree.conflicts_matching(&matcher).collect_vec();
            writeln!(
                formatter.labeled("warning").with_heading("Warning: "),
                "There are unresolved conflicts at these paths:"
            )?;
            print_conflicted_paths(conflicts, formatter, &workspace_command)?;

            let wc_revset = RevsetExpression::commit(status.commit.id().clone());

            // Ancestors with conflicts, excluding the current working copy commit.
            let ancestors_conflicts: Vec<_> = workspace_command
                .attach_revset_evaluator(
                    wc_revset
                        .parents()
                        .ancestors()
                        .filtered(RevsetFilterPredicate::HasConflict)
                        .minus(&workspace_command.env().immutable_expression()),
                )
                .evaluate_to_commit_ids()?
                .try_collect()
                .await?;

            workspace_command
                .report_repo_conflicts(formatter, repo, ancestors_conflicts)
                .await?;
        } else {
            for parent in &status.parents {
                if parent.has_conflict() {
                    writeln!(
                        formatter.labeled("hint").with_heading("Hint: "),
                        "Conflict in parent commit has been resolved in working copy."
                    )?;
                    break;
                }
            }
        }
    } else {
        writeln!(formatter, "No working copy.")?;
    }

    let conflicted_local_bookmarks = repo
        .view()
        .local_bookmarks()
        .filter(|(_, target)| target.has_conflict())
        .map(|(bookmark_name, _)| bookmark_name)
        .collect_vec();
    let conflicted_remote_bookmarks = repo
        .view()
        .all_remote_bookmarks()
        .filter(|(_, remote_ref)| remote_ref.target.has_conflict())
        .map(|(symbol, _)| symbol)
        .collect_vec();
    if !conflicted_local_bookmarks.is_empty() {
        writeln!(
            formatter.labeled("warning").with_heading("Warning: "),
            "These bookmarks have conflicts:"
        )?;
        for name in conflicted_local_bookmarks {
            write!(formatter, "  ")?;
            write!(formatter.labeled("bookmark"), "{}", name.as_symbol())?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter.labeled("hint").with_heading("Hint: "),
            "Use `jj bookmark list` to see details. Use `jj bookmark set <name> -r <rev>` to \
             resolve."
        )?;
    }
    if !conflicted_remote_bookmarks.is_empty() {
        writeln!(
            formatter.labeled("warning").with_heading("Warning: "),
            "These remote bookmarks have conflicts:"
        )?;
        for symbol in conflicted_remote_bookmarks {
            write!(formatter, "  ")?;
            write!(formatter.labeled("bookmark"), "{symbol}")?;
            writeln!(formatter)?;
        }
        writeln!(
            formatter.labeled("hint").with_heading("Hint: "),
            "Use `jj bookmark list` to see details. Resolve by fetching an updated bookmark from \
             the remote."
        )?;
    }

    Ok(())
}

#[cfg(feature = "git")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitSubmoduleCheckoutState {
    Healthy,
    NotCheckedOut,
    Invalid,
}

#[cfg(feature = "git")]
#[derive(Debug)]
struct GitSubmoduleStatus {
    path: RepoPathBuf,
    recorded_id: Option<CommitId>,
    previous_id: Option<CommitId>,
    index_id: Option<CommitId>,
    head_id: Option<CommitId>,
    checkout_state: GitSubmoduleCheckoutState,
    dirty: Option<bool>,
    branch: Option<String>,
    nested_workspace: bool,
}

#[cfg(feature = "git")]
impl GitSubmoduleStatus {
    fn has_checkout_changes(&self) -> bool {
        if self.nested_workspace && self.checkout_state == GitSubmoduleCheckoutState::Healthy {
            // The nested Jujutsu working-copy commit is the recorded gitlink.
            // Git HEAD and the Git index intentionally remain at its parent.
            return false;
        }
        self.checkout_state != GitSubmoduleCheckoutState::Healthy
            || self.dirty != Some(false)
            || matches!(
                (&self.recorded_id, &self.head_id),
                (Some(recorded_id), Some(head_id)) if recorded_id != head_id
            )
            || matches!((&self.recorded_id, &self.head_id), (Some(_), None))
            || matches!(
                (&self.index_id, &self.recorded_id),
                (Some(index_id), Some(recorded_id)) if index_id != recorded_id
            )
    }
}

#[cfg(feature = "git")]
fn collect_git_submodule_statuses(
    tree: &MergedTree,
    parent_tree: &MergedTree,
    workspace_root: &Path,
    git_executable: &Path,
) -> Result<Vec<GitSubmoduleStatus>, CommandError> {
    let current_submodules = collect_tree_gitlinks(tree)?;
    let parent_submodules = collect_tree_gitlinks(parent_tree)?;
    let mut statuses = Vec::new();
    let mut visited = BTreeSet::new();
    for (path, recorded_id) in current_submodules {
        let disk_path = path
            .to_fs_path(workspace_root)
            .map_err(|err| user_error_with_message("Invalid Git submodule path", err))?;
        collect_git_submodule_checkout_status(
            workspace_root,
            path.clone(),
            disk_path,
            Some(recorded_id),
            parent_submodules.get(&path).cloned(),
            None,
            git_executable,
            &mut visited,
            &mut statuses,
        )?;
    }
    statuses.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(statuses)
}

#[cfg(feature = "git")]
fn collect_tree_gitlinks(
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

#[cfg(feature = "git")]
#[expect(clippy::too_many_arguments)]
fn collect_git_submodule_checkout_status(
    workspace_root: &Path,
    path: RepoPathBuf,
    disk_path: PathBuf,
    recorded_id: Option<CommitId>,
    previous_id: Option<CommitId>,
    index_id: Option<CommitId>,
    git_executable: &Path,
    visited: &mut BTreeSet<PathBuf>,
    statuses: &mut Vec<GitSubmoduleStatus>,
) -> Result<(), CommandError> {
    if !disk_path.join(".git").exists() {
        statuses.push(GitSubmoduleStatus {
            path,
            recorded_id,
            previous_id,
            index_id,
            head_id: None,
            checkout_state: GitSubmoduleCheckoutState::NotCheckedOut,
            dirty: None,
            branch: None,
            nested_workspace: false,
        });
        return Ok(());
    }

    let canonical_path = match dunce::canonicalize(&disk_path) {
        Ok(path) if path.starts_with(workspace_root) => path,
        _ => {
            statuses.push(GitSubmoduleStatus {
                path,
                recorded_id,
                previous_id,
                index_id,
                head_id: None,
                checkout_state: GitSubmoduleCheckoutState::Invalid,
                dirty: None,
                branch: None,
                nested_workspace: false,
            });
            return Ok(());
        }
    };
    if !visited.insert(canonical_path.clone()) {
        return Ok(());
    }

    let repo = match gix::open(&canonical_path) {
        Ok(repo) => repo,
        Err(_) => {
            statuses.push(GitSubmoduleStatus {
                path,
                recorded_id,
                previous_id,
                index_id,
                head_id: None,
                checkout_state: GitSubmoduleCheckoutState::Invalid,
                dirty: None,
                branch: None,
                nested_workspace: false,
            });
            return Ok(());
        }
    };
    let head_id = repo
        .head_id()
        .ok()
        .map(|id| CommitId::from_bytes(id.as_bytes()));
    let branch = repo
        .head_name()
        .ok()
        .flatten()
        .map(|name| name.shorten().to_string());
    let dirty = git_repository_is_dirty(git_executable, &canonical_path).ok();
    let nested_workspace = canonical_path.join(".jj").is_dir();
    statuses.push(GitSubmoduleStatus {
        path: path.clone(),
        recorded_id,
        previous_id,
        index_id,
        head_id,
        checkout_state: GitSubmoduleCheckoutState::Healthy,
        dirty,
        branch,
        nested_workspace,
    });

    let Some(submodules) = repo.submodules().ok().flatten() else {
        return Ok(());
    };
    for submodule in submodules {
        let Ok(child_path) = submodule.path() else {
            continue;
        };
        let child_path = gix::path::from_bstring(child_path);
        let child_repo_path = match RepoPathBuf::from_relative_path(
            &PathBuf::from(path.as_internal_file_string()).join(&child_path),
        ) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let child_disk_path = child_repo_path
            .to_fs_path(workspace_root)
            .map_err(|err| user_error_with_message("Invalid nested Git submodule path", err))?;
        let child_recorded_id = submodule
            .head_id()
            .ok()
            .flatten()
            .map(|id| CommitId::from_bytes(id.as_bytes()));
        let child_index_id = submodule
            .index_id()
            .ok()
            .flatten()
            .map(|id| CommitId::from_bytes(id.as_bytes()));
        collect_git_submodule_checkout_status(
            workspace_root,
            child_repo_path,
            child_disk_path,
            child_recorded_id,
            None,
            child_index_id,
            git_executable,
            visited,
            statuses,
        )?;
    }
    Ok(())
}

#[cfg(feature = "git")]
fn git_repository_is_dirty(git_executable: &Path, repo_root: &Path) -> Result<bool, String> {
    let output = std::process::Command::new(git_executable)
        .current_dir(repo_root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ])
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(!output.stdout.is_empty())
}

#[cfg(feature = "git")]
fn print_git_submodule_statuses(
    formatter: &mut dyn Formatter,
    workspace_command: &WorkspaceCommandHelper,
    statuses: &[GitSubmoduleStatus],
    matcher: &dyn Matcher,
    nested_under_changes: bool,
) -> Result<(), CommandError> {
    let matching_statuses = statuses
        .iter()
        .filter(|status| matcher.matches(&status.path))
        .collect_vec();
    if matching_statuses.is_empty() {
        return Ok(());
    }

    let (heading_indent, entry_indent) = if nested_under_changes {
        ("  ", "    ")
    } else {
        ("", "  ")
    };
    writeln!(formatter, "{heading_indent}Submodules:")?;
    for status in matching_statuses {
        let ui_path = workspace_command
            .path_converter()
            .format_file_path(&status.path);
        write!(formatter, "{entry_indent}{ui_path}")?;
        if let Some(recorded_id) = &status.recorded_id {
            write!(formatter, " {}", short_commit_hash(recorded_id))?;
        }

        let mut details = Vec::new();
        match status.checkout_state {
            GitSubmoduleCheckoutState::Healthy => {
                if status.nested_workspace {
                    if status.dirty == Some(false) && status.recorded_id == status.head_id {
                        details.push("clean nested working copy".to_owned());
                    } else {
                        details.push("nested working copy".to_owned());
                    }
                } else if status.head_id.is_none() {
                    details.push("checkout has no HEAD".to_owned());
                    if status.dirty == Some(true) {
                        details.push("dirty".to_owned());
                    } else if status.dirty.is_none() {
                        details.push("working-copy status unavailable".to_owned());
                    }
                } else {
                    match status.dirty {
                        Some(true) => details.push("dirty".to_owned()),
                        Some(false) => details.push("clean".to_owned()),
                        None => details.push("working-copy status unavailable".to_owned()),
                    }
                    if let (Some(recorded_id), Some(head_id)) =
                        (&status.recorded_id, &status.head_id)
                        && recorded_id != head_id
                    {
                        details.push(format!("checked out at {}", short_commit_hash(head_id)));
                    }
                }
            }
            GitSubmoduleCheckoutState::NotCheckedOut => {
                details.push("not checked out".to_owned());
            }
            GitSubmoduleCheckoutState::Invalid => {
                details.push("invalid Git checkout".to_owned());
            }
        }
        if let (Some(previous_id), Some(recorded_id)) = (&status.previous_id, &status.recorded_id)
            && previous_id != recorded_id
        {
            details.push(format!(
                "gitlink changed from {}",
                short_commit_hash(previous_id)
            ));
        }
        if let (Some(index_id), Some(recorded_id)) = (&status.index_id, &status.recorded_id)
            && index_id != recorded_id
        {
            details.push(format!(
                "Git index points to {}",
                short_commit_hash(index_id)
            ));
        }
        if let Some(branch) = &status.branch {
            details.push(format!("branch {branch}"));
        }
        if !details.is_empty() {
            write!(formatter, " ({})", details.join("; "))?;
        }
        writeln!(formatter)?;
    }
    Ok(())
}

struct WorkingCopyStatus {
    commit: Commit,
    parents: Vec<Commit>,
    parent_tree: MergedTree,
    tree: MergedTree,
    untracked_paths: BTreeMap<RepoPathBuf, UntrackedReason>,
}

impl WorkingCopyStatus {
    fn has_any_tracked_changes(&self) -> bool {
        self.tree.tree_ids() != self.parent_tree.tree_ids()
    }

    fn has_any_untracked_paths(&self) -> bool {
        !self.untracked_paths.is_empty()
    }

    fn untracked_paths_matching(&self, matcher: &dyn Matcher) -> impl Iterator<Item = &RepoPath> {
        self.untracked_paths
            .keys()
            .filter(|path| matcher.matches(path))
            .map(|path| path.as_ref())
    }
}

async fn collect_working_copy_status(
    repo: &dyn Repo,
    commit: &Commit,
    snapshot_stats: SnapshotStats,
) -> Result<WorkingCopyStatus, CommandError> {
    let commit = commit.clone();
    let parents = commit.parents().await?;
    let parent_tree = commit.parent_tree(repo).await?;
    let tree = commit.tree();
    let untracked_paths = snapshot_stats.untracked_paths;

    Ok(WorkingCopyStatus {
        commit,
        parents,
        parent_tree,
        tree,
        untracked_paths,
    })
}

async fn visit_collapsed_untracked_files(
    untracked_paths: impl IntoIterator<Item = impl AsRef<RepoPath>>,
    tree: MergedTree,
    mut on_path: impl FnMut(&RepoPath, bool) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let trees = tree.trees().await?;
    let mut stack = vec![trees];

    // TODO: This loop can be improved with BTreeMap cursors once that's stable,
    // would remove the need for the whole `skip_prefixed_by` thing and turn it
    // into a B-tree lookup.
    let mut skip_prefixed_by_dir: Option<RepoPathBuf> = None;
    'untracked: for path in untracked_paths {
        let path = path.as_ref();
        if skip_prefixed_by_dir
            .as_ref()
            .is_some_and(|p| path.starts_with(p))
        {
            continue;
        } else {
            skip_prefixed_by_dir = None;
        }

        let mut it = path.components().dropping_back(1);
        let first_mismatch = it.by_ref().enumerate().find(|(i, component)| {
            stack.get(i + 1).is_none_or(|tree| {
                tree.dir()
                    .components()
                    .next_back()
                    .expect("should always have at least one element (the root)")
                    != *component
            })
        });

        if let Some((i, component)) = first_mismatch {
            stack.truncate(i + 1);
            for component in std::iter::once(component).chain(it) {
                let parent = stack
                    .last()
                    .expect("should always have at least one element (the root)");

                if let Some(subtree) = parent.sub_tree(component).await? {
                    stack.push(subtree);
                } else {
                    let dir = parent.dir().join(component);

                    on_path(&dir, true)?;
                    skip_prefixed_by_dir = Some(dir);

                    continue 'untracked;
                }
            }
        }

        on_path(path, false)?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use pollster::FutureExt as _;
    use testutils::TestRepo;
    use testutils::TestTreeBuilder;
    use testutils::repo_path;

    use super::*;

    fn collect_collapsed_untracked_files_string(
        untracked_paths: &[&RepoPath],
        tree: MergedTree,
    ) -> String {
        let mut result = String::new();
        visit_collapsed_untracked_files(untracked_paths, tree, |path, is_dir| {
            result.push_str("? ");
            if is_dir {
                result.push_str(&path.to_internal_dir_string());
            } else {
                result.push_str(path.as_internal_file_string());
            }
            result.push('\n');
            Ok(())
        })
        .block_on()
        .unwrap();
        result
    }

    #[test]
    fn test_collapsed_untracked_files() {
        let repo = TestRepo::init();

        let tracked = {
            let mut builder = TestTreeBuilder::new(repo.repo.store().clone());

            builder.file(repo_path("top_level_file"), "");
            // ? "untracked_top_level_file"
            // ? "dir"
            // ? "dir2/c"
            builder.file(repo_path("dir2/d"), "");
            // ? "dir3/partially_tracked/e"
            builder.file(repo_path("dir3/partially_tracked/f"), "");
            // ? "dir3/fully_untracked/"
            builder.file(repo_path("dir3/j"), "");
            // ? "dir3/k"

            builder.write_merged_tree()
        };
        let untracked = &[
            repo_path("untracked_top_level_file"),
            repo_path("dir/a"),
            repo_path("dir/b"),
            repo_path("dir2/c"),
            repo_path("dir3/partially_tracked/e"),
            repo_path("dir3/fully_untracked/g"),
            repo_path("dir3/fully_untracked/h"),
            repo_path("dir3/k"),
        ];

        insta::assert_snapshot!(
            collect_collapsed_untracked_files_string(untracked, tracked),
            @"
        ? untracked_top_level_file
        ? dir/
        ? dir2/c
        ? dir3/partially_tracked/e
        ? dir3/fully_untracked/
        ? dir3/k
        "
        );
    }
}
