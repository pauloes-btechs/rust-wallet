//! The CLI end to end over the recorded litd regtest session.

use assert_cmd::Command;
use predicates::prelude::*;

fn rw() -> Command {
    Command::cargo_bin("rw").unwrap()
}

#[test]
fn status_over_replay() {
    rw().args(["--replay", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("phase      Ready"))
        .stdout(predicate::str::contains("regtest"));
}

#[test]
fn balance_is_exact() {
    rw().args(["--replay", "balance"])
        .assert()
        .success()
        .stdout(predicate::str::contains("159,952,157"))
        .stdout(predicate::str::contains("39,109,120"));
}

#[test]
fn json_output_keeps_amounts_as_strings() {
    let out = rw().args(["--replay", "--json", "channels"]).assert().success().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(v["balance"]["localSat"].is_string());
}

#[test]
fn pay_requires_confirmation() {
    rw().args(["--replay", "pay", "lnbcrt1…"])
        .write_stdin("n\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("cancelled"));
}

#[test]
fn intents_need_a_live_node() {
    rw().args(["--replay", "intents"]).assert().failure();
}

#[test]
fn live_mode_requires_credentials() {
    rw().args(["status"]).env_remove("RW_LND_DIR").assert().failure().stderr(predicate::str::contains("--lnd-dir"));
}
