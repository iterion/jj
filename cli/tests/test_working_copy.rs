// Copyright 2023 The Jujutsu Authors
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

use indoc::indoc;
use regex::Regex;
use testutils::TestResult;

use crate::common::TestEnvironment;

#[test]
fn test_snapshot_large_file() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // test a small file using raw-integer-literal syntax, which is interpreted
    // in bytes
    test_env.add_config(r#"snapshot.max-new-file-size = 10"#);
    work_dir.write_file("empty", "");
    work_dir.write_file("large", "a lot of text");
    let output = work_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 13.0B (13 bytes); the maximum size allowed is 10.0B (10 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=13 status`
        This will increase the maximum file size allowed for new files, for this command only.
    [EOF]
    ");

    // test with a larger file using 'KB' human-readable syntax
    test_env.add_config(r#"snapshot.max-new-file-size = "10KB""#);
    let big_string = vec![0; 1024 * 11];
    work_dir.write_file("large", &big_string);
    let output = work_dir.run_jj(["file", "list"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=11264 status`
        This will increase the maximum file size allowed for new files, for this command only.
    [EOF]
    ");

    // test with file track for hint formatting, both files should appear in
    // warnings even though they were snapshotted separately
    work_dir.write_file("large 2", big_string);
    let output = work_dir.run_jj([
        "file",
        "--config=snapshot.auto-track='large'",
        "track",
        "large 2",
    ]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: Refused to snapshot some files:
      large: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
      large 2: 11.0KiB (11264 bytes); the maximum size allowed is 10.0KiB (10240 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 11264`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=11264 file track large 'large 2'`
        This will increase the maximum file size allowed for new files, for this command only.
      * Run `jj file track --include-ignored large 'large 2'`
        This will track the file(s) regardless of size.
    [EOF]
    ");

    // test invalid configuration
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.max-new-file-size=[]"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Config error: Invalid type or value for snapshot.max-new-file-size
    Caused by: Expected a positive integer or a string in '<number><unit>' form
    For help, see https://docs.jj-vcs.dev/latest/config/ or use `jj help -k config`.
    [EOF]
    [exit status: 1]
    ");

    // No error if we disable auto-tracking of the path
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.auto-track='none()'"]);
    insta::assert_snapshot!(output, @"
    empty
    [EOF]
    ");

    // max-new-file-size=0 means no limit
    let output = work_dir.run_jj(["file", "list", "--config=snapshot.max-new-file-size=0"]);
    insta::assert_snapshot!(output, @"
    empty
    large
    large 2
    [EOF]
    ");
}

#[test]
fn test_snapshot_large_file_restore() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");
    test_env.add_config("snapshot.max-new-file-size = 10");

    work_dir.run_jj(["describe", "-mcommitted"]).success();
    work_dir.write_file("file", "small");

    // Write a large file in the working copy, restore it from a commit. The
    // working-copy content shouldn't be overwritten.
    work_dir.run_jj(["new", "root()"]).success();
    work_dir.write_file("file", "a lot of text");
    let output = work_dir.run_jj(["restore", "--from=subject(committed)"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Warning: Refused to snapshot some files:
      file: 13.0B (13 bytes); the maximum size allowed is 10.0B (10 bytes)
    Hint: This is to prevent large files from being added by accident. To fix this:
      * Add the file(s) to `.gitignore`
      * Run `jj config set --repo snapshot.max-new-file-size 13`
        This will increase the maximum file size allowed for new files, in this repository only.
      * Run `jj --config snapshot.max-new-file-size=13 status`
        This will increase the maximum file size allowed for new files, for this command only.
    Working copy  (@) now at: kkmpptxz 119f5156 (no description set)
    Parent commit (@-)      : zzzzzzzz 00000000 (empty) (no description set)
    Added 1 files, modified 0 files, removed 0 files
    Warning: 1 of those updates were skipped because there were conflicting changes in the working copy.
    Hint: Inspect the changes compared to the intended target with `jj diff --from 119f5156d330`.
    Discard the conflicting changes with `jj restore --from 119f5156d330`.
    [EOF]
    ");
    insta::assert_snapshot!(work_dir.read_file("file"), @"a lot of text");

    // However, the next command will snapshot the large file because it is now
    // tracked. TODO: Should we remember the untracked state?
    let output = work_dir.run_jj(["status"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    A file
    Working copy  (@) : kkmpptxz 09eba65e (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");
}

#[test]
fn test_materialize_and_snapshot_different_conflict_markers() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Configure to use Git-style conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "git""#);

    // Create a conflict in the working copy
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "base"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - a
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "side-a"]).success();
    work_dir
        .run_jj(["new", "subject(base)", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - b
            line 3 - b
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)", "subject(side-b)"])
        .success();

    // File should have Git-style conflict markers
    insta::assert_snapshot!(work_dir.read_file("file"), @r#"
    line 1
    <<<<<<< rlvkpnrz df1cdd77 "side-a"
    line 2 - a
    line 3
    ||||||| qpvuntsm 2205b3ac "base"
    line 2
    line 3
    =======
    line 2 - b
    line 3 - b
    >>>>>>> zsuskuln 68dcce1b "side-b"
    "#);

    // Configure to use JJ-style "snapshot" conflict markers
    test_env.add_config(r#"ui.conflict-marker-style = "snapshot""#);

    // Update the conflict, still using Git-style conflict markers
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<<
            line 2 - a
            line 3 - a
            |||||||
            line 2
            line 3
            =======
            line 2 - b
            line 3 - b
            >>>>>>>
        "},
    );

    // Git-style markers should be parsed, then rendered with new config
    insta::assert_snapshot!(work_dir.run_jj(["diff", "--git"]), @r#"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -2,7 +2,7 @@
     <<<<<<< conflict 1 of 1
     +++++++ rlvkpnrz df1cdd77 "side-a"
     line 2 - a
    -line 3
    +line 3 - a
     ------- qpvuntsm 2205b3ac "base"
     line 2
     line 3
    [EOF]
    "#);
    Ok(())
}

#[test]
fn test_snapshot_invalid_ignore_pattern() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Test invalid pattern in .gitignore
    work_dir.write_file(".gitignore", " []\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @"
    Working copy changes:
    A .gitignore
    Working copy  (@) : qpvuntsm c9cf4826 (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");

    // Test invalid UTF-8 in .gitignore
    work_dir.write_file(".gitignore", b"\xff\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @"
    Working copy changes:
    A .gitignore
    Working copy  (@) : qpvuntsm 15f3d11a (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ");
}

#[cfg(unix)]
#[test]
fn test_snapshot_non_utf8_path() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    if testutils::check_strict_utf8_fs(work_dir.root()) {
        eprintln!(
            "Skipping test \"test_snapshot_non_utf8_path\" due to strict UTF-8 filesystem for \
             path {:?}",
            work_dir.root()
        );
        return;
    }

    std::fs::write(work_dir.root().join(OsStr::from_bytes(b"file\xe0")), "").unwrap();
    std::fs::create_dir(work_dir.root().join(OsStr::from_bytes(b"dir\xe0"))).unwrap();
    work_dir.write_file("file", "");

    // The paths that can't be represented as RepoPaths are skipped, and the
    // snapshot succeeds.
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @r#"
    Working copy changes:
    A file
    Working copy  (@) : qpvuntsm 3dcf981e (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ------- stderr -------
    Warning: Skipped some paths because they are not valid UTF-8:
      .: "dir\xE0"
      .: "file\xE0"
    [EOF]
    "#);

    // .gitignore doesn't apply because we can't build a RepoPath to match
    // against, so the paths are still reported.
    work_dir.write_file(".gitignore", b"dir\xe0\nfile\xe0\n");
    insta::assert_snapshot!(work_dir.run_jj(["st"]), @r#"
    Working copy changes:
    A .gitignore
    A file
    Working copy  (@) : qpvuntsm 0fbe2679 (no description set)
    Parent commit (@-): zzzzzzzz 00000000 (empty) (no description set)
    [EOF]
    ------- stderr -------
    Warning: Skipped some paths because they are not valid UTF-8:
      .: "dir\xE0"
      .: "file\xE0"
    [EOF]
    "#);
}

#[test]
fn test_conflict_marker_length_stored_in_working_copy() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    // Create a conflict in the working copy with long markers on one side
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["commit", "-m", "base"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - left
            line 3 - left
        "},
    );
    work_dir.run_jj(["commit", "-m", "side-a"]).success();
    work_dir
        .run_jj(["new", "subject(base)", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            ======= fake marker
            line 2 - right
            ======= fake marker
            line 3
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)", "subject(side-b)"])
        .success();

    // File should be materialized with long conflict markers
    insta::assert_snapshot!(work_dir.read_file("file"), @r#"
    line 1
    <<<<<<<<<<< conflict 1 of 1
    %%%%%%%%%%% diff from: qpvuntsm 2205b3ac "base"
    \\\\\\\\\\\        to: rlvkpnrz ccf9527c "side-a"
    -line 2
    -line 3
    +line 2 - left
    +line 3 - left
    +++++++++++ zsuskuln d7acaf48 "side-b"
    ======= fake marker
    line 2 - right
    ======= fake marker
    line 3
    >>>>>>>>>>> conflict 1 of 1 ends
    "#);

    // The timestamps in the `jj debug local-working-copy` output change, so we want
    // to remove them before asserting the snapshot
    let timestamp_regex = Regex::new(r"\b\d{10,}\b")?;
    let redact_output = |output: String| {
        let output = timestamp_regex.replace_all(&output, "<timestamp>");
        output.into_owned()
    };

    // Working copy should contain conflict marker length
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("ee791f2181026a056ad383d14dbc749ba44e24fedfd39418954871b83d754929147b951bde1fbcca6a8460932e53a6d7ea739bf054ec0bf03607b9370173d6e5")
    Current tree: MergedTree { tree_ids: Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("f56b8223da0dab22b03b8323ced4946329aeb4e0")]), labels: Labeled(["rlvkpnrz ccf9527c \"side-a\"", "qpvuntsm 2205b3ac \"base\"", "zsuskuln d7acaf48 \"side-b\""]), .. }
    Normal { exec_bit: ExecBit(false) }           313 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    [EOF]
    "#);

    // Update the conflict with more fake markers, and it should still parse
    // correctly (the markers should be ignored)
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<<<<<< conflict 1 of 1
            %%%%%%%%%%% diff from base to side #1
            -line 2
            -line 3
            +line 2 - left
            +line 3 - left
            +++++++++++ side #2
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - right
            ======= fake marker
            line 3
            >>>>>>> fake marker
            >>>>>>>>>>> conflict 1 of 1 ends
        "},
    );

    // The file should still be conflicted, and the new content should be saved
    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    M file
    Working copy  (@) : mzvwutvl d31c99cf (conflict) (no description set)
    Parent commit (@-): rlvkpnrz ccf9527c side-a
    Parent commit (@-): zsuskuln d7acaf48 side-b
    Warning: There are unresolved conflicts at these paths:
    file    2-sided conflict
    [EOF]
    ");
    insta::assert_snapshot!(work_dir.run_jj(["diff", "--git"]), @r#"
    diff --git a/file b/file
    --- a/file
    +++ b/file
    @@ -7,8 +7,10 @@
     +line 2 - left
     +line 3 - left
     +++++++++++ zsuskuln d7acaf48 "side-b"
    -======= fake marker
    +<<<<<<< fake marker
    +||||||| fake marker
     line 2 - right
     ======= fake marker
     line 3
    +>>>>>>> fake marker
     >>>>>>>>>>> conflict 1 of 1 ends
    [EOF]
    "#);

    // Working copy should still contain conflict marker length
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("b196f038bbd8cf84508417da8e974874b52202bc98abca08725e946a7a9ea9e011aeddda805c565f49a1e07e43524161caa56fa438de662aba8a6349b5a44be1")
    Current tree: MergedTree { tree_ids: Conflicted([TreeId("381273b50cf73f8c81b3f1502ee89e9bbd6c1518"), TreeId("771f3d31c4588ea40a8864b2a981749888e596c2"), TreeId("3329c18c95f7b7a55c278c2259e9c4ce711fae59")]), labels: Labeled(["rlvkpnrz ccf9527c \"side-a\"", "qpvuntsm 2205b3ac \"base\"", "zsuskuln d7acaf48 \"side-b\""]), .. }
    Normal { exec_bit: ExecBit(false) }           274 <timestamp> Some(MaterializedConflictData { conflict_marker_len: 11 }) "file"
    [EOF]
    "#);

    // Resolve the conflict
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            <<<<<<< fake marker
            ||||||| fake marker
            line 2 - left
            line 2 - right
            ======= fake marker
            line 3 - left
            >>>>>>> fake marker
        "},
    );

    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output, @"
    Working copy changes:
    M file
    Working copy  (@) : mzvwutvl 469d479f (no description set)
    Parent commit (@-): rlvkpnrz ccf9527c side-a
    Parent commit (@-): zsuskuln d7acaf48 side-b
    [EOF]
    ");

    // When the file is resolved, the conflict marker length is removed from the
    // working copy
    let output = work_dir.run_jj(["debug", "local-working-copy"]);
    insta::assert_snapshot!(output.normalize_stdout_with(redact_output), @r#"
    Current operation: OperationId("65b561ae4667b8b9db3f3d6cb2f6bccd087f17b008ba87b976266e641efdab4f12193407e4f6f8bd76b02eff767a6c173ef53e2c0217be389b05779170a257b6")
    Current tree: MergedTree { tree_ids: Resolved(TreeId("6120567b3cb2472d549753ed3e4b84183d52a650")), labels: Unlabeled, .. }
    Normal { exec_bit: ExecBit(false) }           130 <timestamp> None "file"
    [EOF]
    "#);
    Ok(())
}

#[test]
fn test_submodule_ignored() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("sub", "sub");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule commit"])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // There's no particular reason to run this with jj util exec, it's just that
    // the infra makes it easier to run this way.
    let output = work_dir.run_jj([
        "util",
        "exec",
        "--",
        "git",
        "-c",
        // Git normally doesn't allow file:// in submodules.
        "protocol.file.allow=always",
        "submodule",
        "add",
        &format!("{}/submodule", test_env.env_root().display()),
        "sub",
    ]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    Cloning into '$TEST_ENV/repo/sub'...
    done.
    [EOF]
    ");
    // Use git to commit since jj won't play nice with the submodule.
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ])
        .success();

    // This should be empty. We shouldn't track the submodule itself.
    let output = work_dir.run_jj(["diff", "--summary"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    ignoring git submodule at "sub"
    Done importing changes from the underlying Git repo.
    [EOF]
    "#);

    // Switch to a historical commit before the submodule was checked in.
    work_dir.run_jj(["prev"]).success();
    // jj new (or equivalently prev) should always leave you with an empty working
    // copy.
    let output = work_dir.run_jj(["diff", "--summary"]);
    insta::assert_snapshot!(output, @"");
}

#[test]
fn test_snapshot_jjconflict_trees() -> TestResult {
    let test_env = TestEnvironment::default();
    test_env
        .run_jj_in(".", ["git", "init", "repo", "--colocate"])
        .success();
    let work_dir = test_env.work_dir("repo");

    // Create a conflict in the working copy
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2
            line 3
        "},
    );
    work_dir.run_jj(["new", "-m", "side-a"]).success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - left
            line 3 - left
        "},
    );
    work_dir
        .run_jj(["new", "subject(side-a)-", "-m", "side-b"])
        .success();
    work_dir.write_file(
        "file",
        indoc! {"
            line 1
            line 2 - right
            line 3
        "},
    );
    work_dir.run_jj(["new"]).success();
    work_dir
        .run_jj(["rebase", "-s", "subject(side-b)", "-o", "subject(side-a)"])
        .success();

    // Run `git reset --hard HEAD` to simulate checking out the branch with Git.
    let output = std::process::Command::new("git")
        .current_dir(work_dir.root())
        .args(["reset", "--hard", "HEAD"])
        .output()?;
    assert!(output.status.success());

    // We should see a warning regarding '.jjconflict' trees being checked out.
    let output = work_dir.run_jj(["st"]);
    insta::assert_snapshot!(output.to_string().replace('\\', "/"), @r"
    Working copy changes:
    A .jjconflict-base-0/file
    A .jjconflict-side-0/file
    A .jjconflict-side-1/file
    A JJ-CONFLICT-README
    M file
    Working copy  (@) : zsuskuln 2681a418 (no description set)
    Parent commit (@-): kkmpptxz aadeb8eb (conflict) side-b
    Hint: Conflict in parent commit has been resolved in working copy.
    [EOF]
    ------- stderr -------
    Warning: The working copy contains '.jjconflict' files. These files are used by `jj` internally and should not be present in the working copy.
    Hint: You may have used a regular `git` command to check out a conflicted commit.
    Hint: You can use `jj abandon` to discard the working copy changes.
    [EOF]
    ");
    Ok(())
}

#[test]
fn test_colocated_checkout_updates_submodule_working_copy() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    submodule_dir.write_file("sub", "v1\n");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule v1"])
        .success();
    submodule_dir.write_file("sub", "v2\n");
    submodule_dir
        .run_jj(["commit", "-m", "Submodule v2"])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");

    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-C",
            "sub",
            "config",
            "protocol.file.allow",
            "always",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule at v2",
        ])
        .success();

    work_dir
        .run_jj([
            "util", "exec", "--", "git", "-C", "sub", "checkout", "HEAD~1",
        ])
        .success();
    work_dir
        .run_jj(["util", "exec", "--", "git", "add", "sub"])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Update submodule to v1",
        ])
        .success();

    work_dir.run_jj(["prev"]).success();
    assert_eq!(work_dir.read_file("sub/sub"), "v2\n");

    work_dir.run_jj(["next"]).success();
    assert_eq!(work_dir.read_file("sub/sub"), "v1\n");
}

#[test]
fn test_sub_runs_jj_in_nested_submodule() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_source = test_env.work_dir("submodule");
    submodule_source.write_file("payload", "initial\n");
    submodule_source
        .run_jj(["commit", "-m", "initial"])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ])
        .success();
    work_dir.run_jj(["status"]).success();

    work_dir.write_file("sub/payload", "nested change\n");
    let output = test_env
        .run_jj_in("repo/sub", ["sub", "-S", ".", "status"])
        .success();
    assert!(work_dir.root().join("sub/.jj").is_dir());
    assert!(
        output.stdout.raw().contains("M payload"),
        "expected nested status to include the dirty file, got:\n{output:?}"
    );
    // Once initialized, the nearest workspace changes from the superproject to
    // the nested repository. The same relative selector should keep working.
    let output = test_env
        .run_jj_in("repo/sub", ["sub", "-S", ".", "status"])
        .success();
    assert!(
        output.stdout.raw().contains("M payload"),
        "expected -S . to select the initialized nested repository, got:\n{output:?}"
    );
    let output = work_dir.run_jj(["sub", "-S", "sub", "status"]).success();
    assert!(
        output.stdout.raw().contains("M payload"),
        "expected -S to select the nested repository, got:\n{output:?}"
    );

    // The nested working-copy commit itself is now the outer gitlink. This
    // makes the submodule a normal, committable outer working-copy change even
    // before the nested change has been described with `jj commit`.
    let nested_working_copy_id = test_env
        .run_jj_in(
            "repo/sub",
            [
                "--ignore-working-copy",
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                "commit_id",
            ],
        )
        .success()
        .stdout
        .raw()
        .to_owned();
    let output = work_dir.run_jj(["status"]).success();
    assert!(
        output.stdout.raw().contains("Working copy changes:\nM sub"),
        "expected the nested working set in normal outer changes, got:\n{output:?}"
    );
    let output = work_dir.run_jj(["debug", "tree", "sub"]).success();
    assert!(
        output.stdout.raw().contains(nested_working_copy_id.trim()),
        "expected outer gitlink to point at nested @, got:\n{output:?}"
    );
    work_dir
        .run_jj(["commit", "-m", "capture nested working set"])
        .success();
    let output = work_dir
        .run_jj(["debug", "tree", "-r", "@-", "sub"])
        .success();
    assert!(
        output.stdout.raw().contains(nested_working_copy_id.trim()),
        "expected outer commit to retain the exact nested working set, got:\n{output:?}"
    );
    let output = work_dir.run_jj(["status"]).success();
    assert!(
        output
            .stdout
            .raw()
            .contains("The working copy has no changes."),
        "expected the captured nested working set to be clean outside, got:\n{output:?}"
    );
    assert!(output.stdout.raw().contains("nested working copy"));

    let captured_outer_change = work_dir
        .run_jj(["log", "--no-graph", "-r", "@-", "-T", "change_id"])
        .success()
        .stdout
        .raw()
        .to_owned();
    work_dir.run_jj(["edit", "@--"]).success();
    assert_eq!(work_dir.read_file("sub/payload"), "initial\n");
    work_dir
        .run_jj(["edit", captured_outer_change.trim()])
        .success();
    assert_eq!(work_dir.read_file("sub/payload"), "nested change\n");

    test_env
        .run_jj_in("repo/sub", ["s", "commit", "-m", "nested change"])
        .success();
    let output = work_dir.run_jj(["diff", "--git"]).success();
    assert!(
        output.stdout.raw().contains("diff --git a/sub b/sub"),
        "expected the outer working copy to record the nested commit, got:\n{output:?}"
    );
    let output = test_env
        .run_jj_in(
            "repo/sub",
            [
                "log",
                "--no-graph",
                "-r",
                "@-",
                "-T",
                "description.first_line()",
            ],
        )
        .success();
    assert_eq!(output.stdout.raw(), "nested change");
}

#[test]
fn test_sub_reset_preserves_nested_history_and_outer_changes() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_source = test_env.work_dir("submodule");
    submodule_source.write_file("payload", "initial\n");
    submodule_source
        .run_jj(["commit", "-m", "initial"])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule",
        ])
        .success();
    work_dir.run_jj(["status"]).success();
    work_dir.write_file("outer", "base\n");
    work_dir.run_jj(["commit", "-m", "outer base"]).success();

    work_dir.write_file("outer", "unrelated outer change\n");
    work_dir.write_file("sub/payload", "valuable nested change\n");
    test_env
        .run_jj_in(
            "repo/sub",
            ["sub", "commit", "-m", "valuable nested change"],
        )
        .success();
    let nested_change_id = test_env
        .run_jj_in(
            "repo/sub",
            ["log", "--no-graph", "-r", "@-", "-T", "change_id"],
        )
        .success()
        .stdout
        .raw()
        .to_owned();
    let summary = work_dir.run_jj(["diff", "--summary"]).success();
    assert!(summary.stdout.raw().contains("M outer"));
    assert!(summary.stdout.raw().contains("M sub"));

    test_env.run_jj_in("repo/sub", ["sub", "--reset"]).success();
    assert!(!work_dir.root().join("sub/.jj").exists());
    assert_eq!(work_dir.read_file("sub/payload"), "initial\n");
    let summary = work_dir.run_jj(["diff", "--summary"]).success();
    assert!(summary.stdout.raw().contains("M outer"));
    assert!(!summary.stdout.raw().lines().any(|line| line == "M sub"));

    let backups_root = work_dir.root().join(".jj/submodule-backups");
    let backup_run = std::fs::read_dir(&backups_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let backup_jj = backup_run.join("sub/.jj");
    assert!(backup_jj.is_dir());

    // Moving the metadata back makes the nested change addressable again.
    std::fs::rename(backup_jj, work_dir.root().join("sub/.jj")).unwrap();
    let output = test_env
        .run_jj_in(
            "repo/sub",
            [
                "--ignore-working-copy",
                "show",
                "-r",
                nested_change_id.trim(),
            ],
        )
        .success();
    assert!(output.stdout.raw().contains("valuable nested change"));
}

#[test]
fn test_sub_reset_all_preserves_descendant_nested_history() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "child"])
        .success();
    let child_source = test_env.work_dir("child");
    child_source.write_file("payload", "initial\n");
    child_source.run_jj(["commit", "-m", "initial"]).success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "middle"])
        .success();
    let middle_source = test_env.work_dir("middle");
    middle_source
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/child", test_env.env_root().display()),
            "deep",
        ])
        .success();
    middle_source
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add deep submodule",
        ])
        .success();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/middle", test_env.env_root().display()),
            "sub",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add middle submodule",
        ])
        .success();
    work_dir.run_jj(["status"]).success();

    work_dir.write_file("sub/deep/payload", "valuable deep change\n");
    work_dir
        .run_jj([
            "sub",
            "-S",
            "sub",
            "sub",
            "-S",
            "deep",
            "commit",
            "-m",
            "valuable deep change",
        ])
        .success();
    assert!(work_dir.root().join("sub/.jj").is_dir());
    assert!(work_dir.root().join("sub/deep/.jj").is_dir());
    let deep_change_id = test_env
        .run_jj_in(
            "repo/sub/deep",
            ["log", "--no-graph", "-r", "@-", "-T", "change_id"],
        )
        .success()
        .stdout
        .raw()
        .to_owned();

    work_dir.run_jj(["sub", "--reset-all"]).success();
    assert!(!work_dir.root().join("sub/.jj").exists());
    assert!(!work_dir.root().join("sub/deep/.jj").exists());
    assert_eq!(work_dir.read_file("sub/deep/payload"), "initial\n");

    let backups_root = work_dir.root().join(".jj/submodule-backups");
    let backup_run = std::fs::read_dir(&backups_root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert!(backup_run.join("sub/.jj").is_dir());
    let backup_deep_jj = backup_run.join("sub/deep/.jj");
    assert!(backup_deep_jj.is_dir());

    std::fs::rename(backup_deep_jj, work_dir.root().join("sub/deep/.jj")).unwrap();
    test_env
        .run_jj_in(
            "repo/sub/deep",
            ["--ignore-working-copy", "show", "-r", deep_change_id.trim()],
        )
        .success();
}

#[test]
fn test_colocated_snapshot_records_submodule_head_change() {
    let test_env = TestEnvironment::default();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "submodule"])
        .success();
    let submodule_dir = test_env.work_dir("submodule");
    for version in ["v1", "v2", "v3"] {
        submodule_dir.write_file("payload", format!("{version}\n"));
        submodule_dir.run_jj(["commit", "-m", version]).success();
    }
    submodule_dir
        .run_jj(["bookmark", "create", "topic", "-r", "@--"])
        .success();
    submodule_dir
        .run_jj(["tag", "set", "release", "-r", "@---"])
        .success();
    let submodule_v1 = submodule_dir
        .run_jj(["util", "exec", "--", "git", "rev-parse", "HEAD~2"])
        .success()
        .stdout
        .raw()
        .trim()
        .to_owned();
    let submodule_v2 = submodule_dir
        .run_jj(["util", "exec", "--", "git", "rev-parse", "HEAD~1"])
        .success()
        .stdout
        .raw()
        .trim()
        .to_owned();

    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "repo"])
        .success();
    let work_dir = test_env.work_dir("repo");
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &format!("{}/submodule", test_env.env_root().display()),
            "sub",
        ])
        .success();
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test user",
            "commit",
            "-m",
            "Add submodule at v3",
        ])
        .success();

    for (revision, expected_id) in [
        (submodule_v1.as_str(), submodule_v1.as_str()),
        ("origin/topic", submodule_v2.as_str()),
        ("release", submodule_v1.as_str()),
    ] {
        work_dir
            .run_jj([
                "util", "exec", "--", "git", "-C", "sub", "checkout", revision,
            ])
            .success();
        work_dir.run_jj(["status"]).success();

        let output = work_dir.run_jj(["diff", "--git"]).success();
        let diff = output.stdout.raw();
        assert!(
            diff.contains("diff --git a/sub b/sub"),
            "expected a gitlink diff after checking out {revision}, got:\n{diff}"
        );
        assert!(
            diff.contains(&expected_id[..10]),
            "expected gitlink to point to {expected_id} after checking out {revision}, got:\n{diff}"
        );
    }
}
