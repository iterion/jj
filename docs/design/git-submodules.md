# Git submodules

This is an aspirational document that describes how jj _will_ support Git
submodules. Readers are assumed to have some familiarity with Git and Git
submodules.

This document is a work in progress; submodules are a big feature, and relevant
details will be filled in incrementally.

## Current command behavior

Jujutsu has experimental submodule support in colocated, Git-backed workspaces.
The support described in this section is intentionally narrower than the full
design below, but it covers a useful end-to-end workflow.

### Switching commits

Any command that moves the working copy to another commit, such as `jj edit`,
`jj new`, `jj prev`, or `jj next`, synchronizes the submodule working copies as
if the following command had been run:

```shell
git submodule update --init --recursive
```

Jujutsu then checks out the gitlink IDs from its working-copy tree. This last
step matters in a colocated workspace because the Git index represents the
parent of the Jujutsu working-copy commit. As with Git's default submodule
update mode, the resulting submodule checkouts usually have detached HEADs.
When moving away from a captured nested working set, Jujutsu may force the Git
checkout only at initialized `.jj` workspaces because their prior contents are
already stored as nested commits. Git-only local changes retain Git's normal
overwrite protection. The same rule is applied recursively.

`jj status` includes every configured, checked-out Git submodule recursively,
including clean entries. When an initialized nested Jujutsu working copy
changes, its effective commit becomes the outer gitlink, so the path appears in
the ordinary `Working copy changes` summary (for example, `M format`). The
indented `Submodules` details distinguish captured nested working copies from
raw Git dirt, unexpected checked-out commits, branch checkouts, invalid
repositories, and submodules which have not been checked out.

If synchronization fails, the working-copy move still completes and Jujutsu
prints a warning and the Git error. After correcting authentication, transport,
or local-change problems, use `jj sub status` from inside the submodule to
inspect its own working-copy changes before retrying the Jujutsu command.

### Working in a submodule

`jj sub` (also available as the short alias `jj s`) runs an ordinary Jujutsu
command in a Git submodule. On first use, it initializes a separate colocated
Jujutsu repository in that submodule. This gives the submodule its own working
copy, commits, bookmarks, and operation log instead of mixing its files into the
superproject tree.

From inside the submodule, its path is inferred:

```shell
cd path/to/submodule
jj sub status
jj sub diff
jj sub commit -m 'Describe the nested change'
```

From the superproject root, select the submodule with `-S`:

```shell
jj sub -S path/to/submodule status
```

Every ordinary outer snapshot first snapshots initialized nested workspaces,
deepest-first. If the nested `@` contains changes, its commit ID becomes the
superproject's gitlink. An empty, single-parent `@` collapses to `@-`, avoiding
a meaningless gitlink change after initialization or `jj commit`. Consequently
an outer `jj commit` captures the exact nested working set even before the
nested change is given a description.

### Recovering a nested checkout

If a nested Jujutsu operation leaves a submodule checkout or the outer gitlink
in an unwanted state, reset one submodule from the superproject root:

```shell
jj sub -S path/to/submodule --reset
```

Inside a submodule, its path can be inferred with `jj sub --reset`. To reset
every checked-out submodule, run `jj sub --reset-all` from either the
superproject or a nested submodule. Descendant submodules are included: they
are snapshotted and backed up deepest-first before Git recursively restores the
checkout hierarchy.

Reset restores only gitlink paths from the parent of the superproject's
working-copy commit, so unrelated outer changes are kept. Before forcing the
Git checkouts back to those commits, Jujutsu snapshots each nested workspace
and moves its `.jj` directory below `.jj/submodule-backups/` in the
superproject. Moving a saved `<submodule>/.jj` directory back into place
restores access to its commits and operation log.

### Pointing a submodule at a commit, branch, or tag

Create a nested working-copy commit on the desired revision:

```shell
jj sub -S path/to/submodule new <commit-or-tag>
```

A local or remote bookmark works as well. Because the new nested `@` is empty,
Jujutsu records its parent as the new gitlink value. `jj status` reports the
submodule path as modified and `jj diff --git` shows a gitlink change. The
recorded value remains stable across later Jujutsu commands.

### Recording and pushing a change made in a submodule

A gitlink stores a commit ID. In Jujutsu, the nested working copy is already a
real commit, so the outer working-copy commit can point directly to the dirty
nested `@` and preserve that working set. For a reviewable, pushable change,
describe the nested change first and then describe and push the superproject
change:

```shell
# edit files
jj sub -S path/to/submodule commit -m 'Describe the submodule change'
jj describe -m 'Update path/to/submodule'
jj bookmark create feature -r @
jj git push --bookmark feature
```

Before pushing the superproject reference, `jj git push` pushes each changed,
checked-out submodule to its own configured upstream. Nested Git submodules are
pushed with Git's `--recurse-submodules=on-demand` behavior. A failure to push a
submodule stops the command before the superproject reference is pushed.
`jj git push --dry-run` does not push either repository.

Jujutsu's synchronization normally detaches submodule HEAD. In that state,
`jj git push` looks for a single local branch with an upstream that points to
the recorded gitlink and pushes that branch explicitly. If the commit is
already reachable from a known remote-tracking ref or tag, no submodule push is
needed. Ambiguous branches or a new detached commit without such a branch are
reported as errors instead of guessing a destination.

### Rebasing a moved submodule

If one side of a rebase moves a submodule to a new path while the rebased change
updates that submodule's gitlink at the old path, `jj rebase` carries the
gitlink update to the new path. Unrelated delete/add pairs are not treated as a
move.

### Current limitations

- This workflow requires a colocated, Git-backed workspace.
- Adding a new submodule still uses Git's `git submodule add` and must create a
  Git commit before Jujutsu can import the initial gitlink.
- The nested Jujutsu repository has its own operation log. Coordinating nested
  and superproject operations, including a single atomic undo, remains future
  work.
- Fetching, conflict resolution, and native non-Git Jujutsu submodules remain
  future work.

## Objective

This proposal aims to replicate the workflows users are used to with Git
submodules, e.g.:

- Cloning submodules
- Making new submodule commits and updating the superproject
- Fetching and pushing updates to the submodule's remote
- Viewing submodule history

When it is convenient, this proposal will also aim to make submodules easier to
use than Git's implementation.

### Non-goals

- Non-Git 'submodules' (e.g. native jj submodules, other VCSes)
- Non-Git backends (e.g. Google internal backend)
- Changing how Git submodules are implemented in Git

## Background

We mainly want to support Git submodules for feature parity, since Git
submodules are a standard feature in Git and are popular enough that we have
received user requests for them. Secondarily (and distantly so), Git submodules
are notoriously difficult to use, so there is an opportunity to improve the UX
over Git's implementation.

### Intro to Git Submodules

[Git submodules](https://git-scm.com/docs/gitsubmodules) are a feature of Git
that allow a repository (submodule) to be embedded inside another repository
(the superproject). Notably, a submodule is a full repository, complete with its
own index, object store and ref store. It can be interacted with like any other
repository, regardless of the superproject.

In a superproject commit, submodule information is captured in two places:

- A `gitlink` entry in the commit's tree, where the value of the `gitlink` entry
  is the submodule commit id. This tells Git what to populate in the working
  tree.

- A top level `.gitmodules` file. This file is in Git's config syntax and
  entries take the form `submodule.<submodule-name>.*`. These include many
  settings about the submodules, but most importantly:

  - `submodule<submodule-name>.path` contains the path from the root of the tree
    to the `gitlink` being described.

  - `submodule<submodule-name>.url` contains the url to clone the submodule
    from.

In the working tree, Git notices the presence of a submodule by the `.git` entry
(signifying the root of a Git repository working tree). This is either the
submodule's actual Git directory (an "old-form" submodule), or a `.git` file
pointing to `<superproject-git-directory>/modules/<submodule-name>`. The latter
is sometimes called the "absorbed form", and is Git's preferred mode of
operation.

## Roadmap

Git submodules should be implemented in an order that supports an increasing set
of workflows, with the goal of getting feedback early and often. When support is
incomplete, jj should not crash, but instead provide fallback behavior and warn
the user where needed.

The goal is to land good support for pure Jujutsu workspaces, while colocated
workspaces will be supported when convenient.

This section should be treated as a set of guidelines, not a strict order of
work.

### Phase 1: Readonly submodules

This includes work that inspects submodule contents but does not create new
objects in the submodule. This requires a way to store submodules in a jj
repository that supports readonly operations.

#### Outcomes

- Submodules can be cloned anew
- New submodule commits can be fetched
- Submodule history and branches can be viewed
- Submodule contents are populated in the working copy
- Superproject gitlink can be updated to an existing submodule commit
- Conflicts in the superproject gitlink can be resolved to an existing submodule
  commit

### Phase 2: Snapshotting new changes

This allows a user to write new contents to a submodule and its remote.

#### Outcomes

- Changes in the working copy can be recorded in a submodule commit
- Submodule branches can be modified
- Submodules and their branches can be pushed to their remote

### Phase 3: Merging/rebasing/conflicts

This allows merging and rebasing of superproject commits in a content-aware way
(in contrast to Git, where only the gitlink commit ids are compared), as well as
workflows that make resolving conflicts easy and sensible.

This can be done in tandem with Phase 2, but will likely require a significant
amount of design work on its own.

#### Outcomes

- Merged/rebased submodules result in merged/rebased working copy content
- Merged/rebased working copy content can be committed, possibly by creating
  sensible merged/rebased submodule commits
- Merge/rebase between submodule and non-submodule gives a sensible result
- Merge/rebase between submodule A and submodule B gives a sensible result

### Phase ?: An ideal world

I.e. outcomes we would like to see if there were no constraints whatsoever.

- Rewriting submodule commits rewrites descendants correctly and updates
  superproject gitlinks.
- Submodule conflicts automatically resolve to the 'correct' submodule commits,
  e.g. a merge between superproject commits creating a merge of the submodule
  commits.
- Nested submodules are as easy to work with as non-nested submodules.
- The operation log captures changes in the submodule.

## Design

### Guiding principles

TODO

### Storing submodules

Possible approaches under discussion. See
[./git-submodule-storage.md](./git-submodule-storage.md).

### Snapshotting new submodule changes

The experimental colocated implementation gives each initialized submodule a
separate nested Jujutsu repository. Before the outer filesystem scan, the CLI
recursively snapshots those repositories. A dirty nested `@` overrides the
checked-out Git HEAD as the gitlink value; an empty one-parent `@` contributes
its parent instead. This makes a nested working set an ordinary outer tree
value and therefore an ordinary diff and commit.

The nested and outer snapshots are currently separate operations. A complete
implementation should define their atomic relationship, including how one
`jj undo` operation spans the hierarchy.

### Merging/rebasing with submodules

TODO
