//! The `skill` subcommand: prints the agent skill built into the binary.

use assert_cmd::assert::OutputAssertExt;
use common::cargo_bin_cmd;
use predicates::prelude::predicate;

mod common;

#[test]
fn outside_a_repository_skill_prints_the_skill_with_this_version() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let mut cmd = cargo_bin_cmd!();
    cmd.arg("skill").current_dir(temp.path());

    cmd.output()?
        .assert()
        .success()
        .stdout(include_str!("../src/skill.md").replace("{{version}}", env!("CARGO_PKG_VERSION")));
    Ok(())
}

#[test]
fn project_with_the_saved_skill_passes() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    std::fs::create_dir(project.path().join(".git"))?;
    let skill_dir = project.path().join(".agents/skills/blockwatch");
    std::fs::create_dir_all(&skill_dir)?;
    let skill = cargo_bin_cmd!().arg("skill").assert().success();
    std::fs::write(skill_dir.join("SKILL.md"), &skill.get_output().stdout)?;

    let mut cmd = cargo_bin_cmd!();
    // The path argument makes the run fail if it skips the file, so it can't pass by accident.
    cmd.arg(".agents/skills/blockwatch/SKILL.md")
        .current_dir(project.path());

    cmd.output()?.assert().success();
    Ok(())
}

#[test]
fn plugin_stub_has_the_description_of_the_skill() {
    fn description(skill: &str) -> &str {
        skill
            .lines()
            .find(|line| line.starts_with("description: "))
            .expect("a skill has a description")
    }

    assert_eq!(
        description(include_str!("../.agents/skills/blockwatch/SKILL.md")),
        description(include_str!("../src/skill.md")),
    );
}

#[test]
fn validation_flag_before_skill_fails() -> anyhow::Result<()> {
    let mut cmd = cargo_bin_cmd!();
    cmd.args(["--verbosity", "full", "skill"]);

    cmd.output()?
        .assert()
        .failure()
        .stdout("")
        .stderr(predicate::str::contains("`skill` subcommand"));
    Ok(())
}
